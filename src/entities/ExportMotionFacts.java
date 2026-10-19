import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
public class ExportMotionFacts {
 static Method owner(Class<?> type,String name) {
  for(Class<?> c=type;c!=null;c=c.getSuperclass())for(Method m:c.getDeclaredMethods())if(m.getName().equals(name))return m;
  return null;
 }
 static String declared(Class<?> type,String name) {
  Method m=owner(type,name);return m==null?"":m.getDeclaringClass().getName();
 }
 static Map<String,Object> headRule(Class<?> type,sun.misc.Unsafe unsafe) throws Exception {
  Method m=owner(type,"getMaxHeadYRot");if(m==null)return null;
  String owner=m.getDeclaringClass().getName();
  if(owner.equals("net.minecraft.world.entity.animal.armadillo.Armadillo"))return Map.of("kind","armadillo","idle",32,"scared",0);
  var fixed=Set.of("net.minecraft.world.entity.Mob","net.minecraft.world.entity.animal.axolotl.Axolotl","net.minecraft.world.entity.animal.camel.Camel","net.minecraft.world.entity.animal.dolphin.Dolphin","net.minecraft.world.entity.animal.frog.Frog","net.minecraft.world.entity.animal.goat.Goat","net.minecraft.world.entity.animal.sniffer.Sniffer","net.minecraft.world.entity.monster.Ravager","net.minecraft.world.entity.monster.Shulker","net.minecraft.world.entity.monster.breeze.Breeze");
  if(!fixed.contains(owner))throw new IllegalStateException("unreviewed Source head rule "+owner);
  m.setAccessible(true);return Map.of("kind","fixed","value",((Number)m.invoke(unsafe.allocateInstance(type))).floatValue());
 }
 static String bodyRule(Class<?> type) {
  if(declared(type,"tickHeadTurn").equals("net.minecraft.world.entity.decoration.ArmorStand"))return "armor_stand";
  String owner=declared(type,"createBodyControl");
  return switch(owner) {
   case "" -> "living";
   case "net.minecraft.world.entity.Mob" -> "standard";
   case "net.minecraft.world.entity.animal.armadillo.Armadillo" -> "armadillo";
   case "net.minecraft.world.entity.animal.camel.Camel" -> "camel";
   case "net.minecraft.world.entity.animal.happyghast.HappyGhast" -> "happy_ghast";
   case "net.minecraft.world.entity.monster.Phantom" -> "phantom";
   case "net.minecraft.world.entity.monster.Shulker" -> "shulker";
   case "net.minecraft.world.entity.monster.creaking.Creaking" -> "creaking";
   default -> throw new IllegalStateException("unreviewed Source body rule "+owner);
  };
 }
 public static void main(String[] args) throws Exception {
  net.minecraft.SharedConstants.tryDetectVersion();net.minecraft.server.Bootstrap.bootStrap();
  JsonObject models=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
  Class<?> mob=Class.forName("net.minecraft.world.entity.Mob");
  Class<?> flying=Class.forName("net.minecraft.world.entity.animal.FlyingAnimal");
  Field field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);var unsafe=(sun.misc.Unsafe)field.get(null);
  var result=new TreeMap<String,Object>();
  for(var row:models.getAsJsonObject("entities").entrySet()) {
   Class<?> c=Class.forName(row.getValue().getAsJsonObject().get("class").getAsString());
   var facts=new TreeMap<String,Object>();facts.put("class",c.getName());facts.put("is_mob",mob.isAssignableFrom(c));facts.put("walk_uses_y",flying.isAssignableFrom(c));
   var methods=new TreeMap<String,Object>();
   for(String name:List.of("getMaxHeadYRot","getMaxHeadRotationRelativeToBody","createBodyControl","setYHeadRot","updateWalkAnimation","tickHeadTurn","getCurrentSwingDuration","getAttackAnim","getAgeScale")) {
    Method m=owner(c,name);if(m!=null)methods.put(name,Map.of("owner",m.getDeclaringClass().getName(),"parameters",Arrays.stream(m.getParameterTypes()).map(Class::getName).toList()));
   }
   facts.put("head_rule",headRule(c,unsafe));
   Method bodyHead=owner(c,"getMaxHeadRotationRelativeToBody");
   if(bodyHead!=null) {
    String o=bodyHead.getDeclaringClass().getName();
    if(o.equals("net.minecraft.world.entity.player.Player"))facts.put("body_head_rule",Map.of("kind","blocking","ordinary",50,"blocking",15));
    else if(o.equals("net.minecraft.world.entity.LivingEntity"))facts.put("body_head_rule",Map.of("kind","fixed","value",50));
    else throw new IllegalStateException("unreviewed Source body-relative head rule "+o);
    facts.put("body_rule",bodyRule(c));
    String walkOwner=declared(c,"updateWalkAnimation");
    facts.put("walk_rule",switch(walkOwner) {
     case "net.minecraft.world.entity.LivingEntity" -> "standard";
     case "net.minecraft.world.entity.animal.frog.Frog" -> "frog";
     case "net.minecraft.world.entity.animal.camel.Camel" -> "camel";
     case "net.minecraft.world.entity.monster.creaking.Creaking" -> "creaking";
     default -> throw new IllegalStateException("unreviewed Source walk rule "+walkOwner);
    });
   }
   facts.put("methods",methods);result.put(row.getKey(),facts);
  }
  var document=new TreeMap<String,Object>();document.put("schema",1);document.put("game_version",models.get("game_version").getAsString());document.put("client_sha256",models.get("client_sha256").getAsString());document.put("entities",result);
  Files.writeString(Path.of(args[1]),new Gson().toJson(document));System.out.println("actual Source motion owner entities="+result.size());
 }
}
